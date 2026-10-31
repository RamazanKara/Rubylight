use super::*;
use butterpollo_core::topology::{Kind, Mode, Position};
fn node(id: &str, active: bool, primary: bool) -> Node {
    Node {
        id: id.into(),
        label: id.into(),
        kind: Kind::Physical,
        active,
        primary,
        desired_position: Position { x: 0, y: 0 },
        mode: Mode {
            width: 1920,
            height: 1080,
            refresh_hz: 60.,
        },
        device_id: id.into(),
    }
}
fn snapshot(nodes: Vec<Node>) -> Snapshot {
    Snapshot {
        version: 1,
        hdr: nodes.iter().map(|n| (n.device_id.clone(), true)).collect(),
        scale: nodes.iter().map(|n| (n.device_id.clone(), 125)).collect(),
        rotation: Default::default(),
        clone_groups: vec![],
        nodes,
    }
}
#[test]
fn reapplying_follows_a_display_windows_listed_under_another_name() {
    let monitor = |device_id: &str, display_name: &str| crate::display::Monitor {
        device_id: device_id.into(),
        monitor_device_path: String::new(),
        display_name: display_name.into(),
        friendly_name: String::new(),
        hdr_supported: false,
        hdr_enabled: false,
        primary: false,
        adapter: Default::default(),
        target: 0,
        source: 0,
    };
    let monitors = [
        monitor("SAM7454", r"\\.\DISPLAY1"),
        monitor("RBY0001", r"\\.\DISPLAY25"),
    ];
    // Issue 56: the stream still knew DISPLAY23.
    let target = reapplied_target(&monitors, r"\\.\DISPLAY23", "RBY0001").unwrap();
    assert_eq!(target.display_name, r"\\.\DISPLAY25");
    assert_eq!(
        reapplied_target(&monitors, r"\\.\DISPLAY1", "RBY0001")
            .unwrap()
            .device_id,
        "SAM7454",
        "a known output name wins"
    );
    assert!(reapplied_target(&monitors, r"\\.\DISPLAY23", "").is_none());
    assert!(reapplied_target(&monitors, r"\\.\DISPLAY23", "RBY0002").is_none());
}
#[test]
fn disabling_an_original_display_requires_restoration_after_a_competing_layout_change() {
    let before = vec![node("physical", true, true), node("tv", true, false)];
    let exclusive = vec![
        node("physical", false, false),
        node("tv", false, false),
        node("vdd", true, true),
    ];
    let user_recovery = vec![node("physical", true, true), node("vdd", true, false)];
    assert!(!matches(&user_recovery, &exclusive));
    assert!(disabled_original_displays(&before, &exclusive));
    let extended = vec![
        node("physical", true, true),
        node("tv", true, false),
        node("vdd", true, false),
    ];
    assert!(!disabled_original_displays(&before, &extended));
    // A display that was already off must not cause a forced restore.
    assert!(!disabled_original_displays(
        &[node("physical", false, false)],
        &exclusive
    ));
    // Adopting an earlier failed restoration can mean the physical
    // target is already absent from the new stream's active snapshot.
    assert!(disabled_original_displays(
        &before,
        &[node("vdd", true, true)]
    ));
}
#[test]
fn an_unplugged_display_does_not_keep_the_layout_from_being_restored() {
    let mut tv = node("tv", true, false);
    tv.desired_position = Position { x: 1920, y: 0 };
    let applied = vec![node("monitor", true, true), tv];
    let current = vec![node("monitor", true, true)];
    let without_tv: std::collections::BTreeSet<String> = ["monitor".to_string()].into();
    assert!(matches_connected(&current, &applied, Some(&without_tv)));
    // Still connected but switched off: the user changed the layout.
    let with_tv: std::collections::BTreeSet<String> =
        ["monitor".to_string(), "tv".to_string()].into();
    assert!(!matches_connected(&current, &applied, Some(&with_tv)));
    assert!(!matches(&current, &applied));
    assert!(matches(&applied, &applied));
}
#[test]
fn a_remote_monitor_switched_on_mid_stream_keeps_the_streams_layout() {
    let mut state = State {
        users: 1,
        before: Some(snapshot(vec![node("phys", true, true)])),
        applied: vec![node("vdd", true, true), node("phys", false, false)],
        ..Default::default()
    };
    let mut remote = node("remote", true, false);
    remote.desired_position = Position { x: 1920, y: 0 };
    let current = vec![node("vdd", true, true), remote];
    let connected: std::collections::BTreeSet<String> =
        ["vdd", "phys", "remote"].map(String::from).into();
    assert!(!matches_connected(
        &current,
        &state.applied,
        Some(&connected)
    ));
    assert!(retain_in(&mut state, &current, "remote", true));
    assert!(matches_connected(
        &current,
        &state.applied,
        Some(&connected)
    ));
    // The restore keeps it on beside the user's own displays.
    let before = state.before.as_ref().unwrap();
    assert!(
        before
            .nodes
            .iter()
            .any(|n| n.device_id == "remote" && n.active)
    );
    assert!(
        before
            .nodes
            .iter()
            .any(|n| n.device_id == "phys" && n.active)
    );
    assert_eq!(state.retained, ["remote"]);
    // Disconnected: no longer kept on, and its removal is no change.
    assert!(!retain_in(&mut state, &[], "remote", false));
    assert!(state.retained.is_empty());
    let current = vec![node("vdd", true, true)];
    let connected: std::collections::BTreeSet<String> = ["vdd", "phys"].map(String::from).into();
    assert!(matches_connected(
        &current,
        &state.applied,
        Some(&connected)
    ));
    // Without a stream holding a layout there is nothing to record.
    let mut idle = State::default();
    assert!(!retain_in(
        &mut idle,
        &[node("remote", true, false)],
        "remote",
        true
    ));
    assert!(idle.applied.is_empty() && idle.retained.is_empty());
}
#[test]
fn a_saved_layout_left_behind_by_a_crash_is_noticed_when_the_virtual_display_arrives() {
    let before = vec![node("phys", true, true), node("tv", false, false)];
    let connected = ["phys", "tv", "vdd"].map(String::from);
    // Windows recalled a stream's saved layout as the display arrived.
    assert!(switched_off(
        &before,
        &[node("vdd", true, true)],
        &connected
    ));
    // As Windows lays out a new display: everything stays on.
    let extended = [node("phys", true, true), node("vdd", true, false)];
    assert!(!switched_off(&before, &extended, &connected));
    // A display unplugged since, or off before the stream, is no sign.
    let unplugged = ["tv", "vdd"].map(String::from);
    assert!(!switched_off(
        &before,
        &[node("vdd", true, true)],
        &unplugged
    ));
    assert!(persist(Arrangement::Exclusive, true));
    assert!(!persist(Arrangement::Exclusive, false));
    assert!(!persist(Arrangement::Extended, true));
}
#[test]
fn the_original_layout_leaves_out_the_streams_virtual_display() {
    let layout = original_layout(
        snapshot(vec![node("monitor", true, true), node("vdd", true, false)]),
        "vdd",
    );
    let vdd = layout.nodes.iter().find(|n| n.device_id == "vdd").unwrap();
    assert!(!vdd.active && !vdd.primary);
    assert!(
        layout
            .nodes
            .iter()
            .any(|n| n.device_id == "monitor" && n.active)
    );
    assert!(!layout.hdr.contains_key("vdd") && !layout.scale.contains_key("vdd"));
    assert!(layout.hdr.contains_key("monitor"));
    // With no other active display the layout cannot exclude it.
    let only = original_layout(
        snapshot(vec![node("monitor", false, false), node("vdd", true, true)]),
        "vdd",
    );
    assert!(only.nodes.iter().any(|n| n.device_id == "vdd" && n.active));
}
