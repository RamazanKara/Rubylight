//! Unit tests for monitor-switch guardrails and CaptureKey isolation.

use super::super::*;

#[test]
fn monitor_switch_allowed_enforces_role_mode_and_keyboard_permission() {
    use butterpollo_input_policy::input::KEYBOARD_PERMISSION;
    let f = crate::state::test_support::Fixture::new();
    let config = Config::default();

    let allowed = f.client(u32::MAX);
    let owner_stream = f.session(allowed.clone(), Role::Stream);
    assert!(monitor_switch_allowed(&owner_stream, &config));

    // Disallowed roles.
    let remote = f.session(allowed.clone(), Role::RemoteMonitor);
    assert!(!monitor_switch_allowed(&remote, &config));
    let input_only = f.session(allowed.clone(), Role::InputOnly);
    assert!(!monitor_switch_allowed(&input_only, &config));

    // Disallowed without keyboard permission.
    let no_kb = f.client(u32::MAX ^ KEYBOARD_PERMISSION);
    let no_kb_session = f.session(no_kb, Role::Stream);
    assert!(!monitor_switch_allowed(&no_kb_session, &config));

    // Disallowed for joined guest sessions.
    let mut guest_launch = f.launch(allowed.clone(), Role::Stream);
    guest_launch.options.insert(
        butterpollo_session::multiclient::LAUNCH_OPTION.into(),
        "7".into(),
    );
    let guest = Session::new(guest_launch, Default::default());
    assert!(!monitor_switch_allowed(&guest, &config));

    // Disallowed for Nucleus Co-op sessions.
    let mut coop_launch = f.launch(allowed, Role::Stream);
    coop_launch.nucleus = Some(butterpollo_session::session::Seat {
        session: 1,
        slot: 0,
    });
    let coop = Session::new(coop_launch, Default::default());
    assert!(!monitor_switch_allowed(&coop, &config));

    // Disallowed in background mode.
    let mut background = Config::default();
    butterpollo_session::background::apply(&mut background);
    assert!(!monitor_switch_allowed(&owner_stream, &background));
}

#[test]
fn capture_key_switch_scope_isolates_independent_owners_and_disallowed_roles() {
    let config = Config::default();
    let base = CaptureKey::new("wgc", r"\\.\DISPLAY1", false, &config, "", false);
    let owner_a = CaptureKey::new("wgc", r"\\.\DISPLAY1", false, &config, "", false)
        .with_switch_scope(Some("owner-a".into()));
    let owner_a_dup = CaptureKey::new("wgc", r"\\.\DISPLAY1", false, &config, "", false)
        .with_switch_scope(Some("owner-a".into()));
    let owner_b = CaptureKey::new("wgc", r"\\.\DISPLAY1", false, &config, "", false)
        .with_switch_scope(Some("owner-b".into()));
    assert_eq!(owner_a, owner_a_dup);
    assert_ne!(owner_a, owner_b);
    assert_ne!(owner_a, base);
}
