//! Regression tests for multi-client audio route ownership (`STR-08`).

use super::retains_app_audio;
use crate::state::test_support::Fixture;
use butterpollo_session::{multiclient::LAUNCH_OPTION, session::Role};

#[test]
fn guest_streams_never_retain_or_overwrite_host_app_audio_route() {
    let f = Fixture::new();
    let owner = f.launch(f.client(u32::MAX), Role::Stream);
    assert!(retains_app_audio(&owner));

    let mut guest = f.launch(f.client(u32::MAX), Role::Stream);
    guest.options.insert(LAUNCH_OPTION.into(), "1".into());
    assert!(!retains_app_audio(&guest));

    let remote_monitor = f.launch(f.client(u32::MAX), Role::RemoteMonitor);
    assert!(!retains_app_audio(&remote_monitor));

    let input_only = f.launch(f.client(u32::MAX), Role::InputOnly);
    assert!(!retains_app_audio(&input_only));
}
