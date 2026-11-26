//! Per-display profile recall (nightjar#163): profiles.json from 6.4 loads and saves unchanged,
//! and a profile kept for a screen is picked only on that screen.

use super::super::*;
use crate::network_types::Kind;

/// profiles.json as Nightjar 6.4 saves it.
const PROFILES_64: &str = r#"{"active":1,"entries":[{"id":1,"name":"Home","settings":{"fps":120}},{"id":2,"name":"Travel","settings":{"bitrate_kbps":8000,"fps":60}}],"automatic":true,"mobile":2,"home":null,"wifi":null,"home_wifi":["Home"],"mobile_limit_kbps":5000}"#;

fn screen(name: &str) -> DisplayMatcher {
    DisplayMatcher::new(name, name).unwrap()
}

#[test]
fn profiles_from_6_4_load_and_save_unchanged() {
    let profiles: Profiles = serde_json::from_str(PROFILES_64).unwrap();
    assert!(profiles.entries.iter().all(|p| p.display_matcher.is_none() && p.resolution_scale_pct.is_none()));
    assert_eq!(serde_json::to_string(&profiles).unwrap(), PROFILES_64);
    // One screen, no profile kept for it: the choice is the one 6.4 made.
    let mut on_screen = profiles.clone();
    on_screen.screen = Some(screen("Built-in Retina Display"));
    for kind in [Kind::Mobile, Kind::HomeWifi, Kind::Wifi, Kind::Other] {
        assert_eq!(on_screen.selected(kind), profiles.selected(kind));
    }
    assert_eq!(profiles.selected(Kind::Mobile), Some(2));
    assert_eq!(profiles.selected(Kind::Other), Some(1));
}

#[test]
fn the_screen_the_window_is_on_calls_up_its_profile() {
    let mut profiles: Profiles = serde_json::from_str(PROFILES_64).unwrap();
    let laptop = profiles.create("Laptop panel", serde_json::from_str(r#"{"native_scale":75,"hdr":false}"#).unwrap()).unwrap();
    let desk = profiles.create("Desk monitor", serde_json::from_str(r#"{"fps":144,"hdr":true}"#).unwrap()).unwrap();
    profiles.keep_for_screen(laptop, Some(&screen("Built-in Retina Display")));
    profiles.keep_for_screen(desk, Some(&screen("DELL U2720Q")));

    profiles.screen = Some(screen("built-in retina display"));
    assert_eq!(profiles.selected(Kind::Mobile), Some(laptop), "ahead of the network choice");
    let settings = profiles.settings(&Settings::default(), Kind::Other);
    assert_eq!((settings.native_scale, settings.hdr), (75, false));

    profiles.screen = Some(screen("DELL U2720Q"));
    let settings = profiles.settings(&Settings::default(), Kind::Other);
    assert_eq!((settings.fps, settings.hdr), (144, true));

    // A screen no profile is kept for, or none known: the network choice and the profile in use.
    profiles.screen = Some(screen("LG HDR 4K"));
    assert_eq!(profiles.selected(Kind::Mobile), Some(2));
    profiles.screen = None;
    assert_eq!(profiles.selected(Kind::Other), Some(1));

    // The screen is never saved, the profiles' screens are, and they read back.
    profiles.screen = Some(screen("DELL U2720Q"));
    let saved = serde_json::to_string(&profiles).unwrap();
    assert!(!saved.contains("\"screen\""));
    let restored: Profiles = serde_json::from_str(&saved).unwrap();
    assert_eq!(restored.entries, profiles.entries);
    assert_eq!(restored.screen, None);
}

#[test]
fn a_screen_keeps_one_profile_and_deleting_a_profile_frees_its_screen() {
    let mut profiles = Profiles::default();
    let a = profiles.create("A", Overrides::default()).unwrap();
    let b = profiles.create("B", Overrides::default()).unwrap();
    let dell = screen("DELL U2720Q");
    profiles.keep_for_screen(a, Some(&dell));
    profiles.keep_for_screen(b, Some(&screen("dell u2720q")));
    assert_eq!(profiles.entries.iter().filter(|p| p.display_matcher.is_some()).count(), 1);
    profiles.screen = Some(dell.clone());
    assert_eq!(profiles.for_screen(), Some(b));
    profiles.keep_for_screen(b, None);
    assert_eq!(profiles.for_screen(), None);
    profiles.keep_for_screen(a, Some(&dell));
    profiles.delete(a);
    assert_eq!(profiles.for_screen(), None);
    assert_eq!(profiles.selected(Kind::Other), None);
}

#[test]
fn a_profile_with_a_screen_reads_as_7_0_saved_it() {
    let saved = r#"{"entries":[{"id":3,"name":"TV","settings":{},"resolution_scale_pct":75,"display_matcher":{"id":"LG TV SSCR2","name":"LG TV SSCR2"}}]}"#;
    let profiles: Profiles = serde_json::from_str(saved).unwrap();
    assert_eq!(profiles.entries[0].resolution_scale_pct, Some(75));
    assert_eq!(profiles.entries[0].display_matcher.as_ref().map(|m| m.id.as_str()), Some("LG TV SSCR2"));
}
