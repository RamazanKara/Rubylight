//! Settings tests for display layers, decoder queue, simple profile, debug visibility, and vibration.

use super::*;

#[test]
fn display_arrangement_holds_virtual_display_and_a_pick_writes_both_keys() {
    use crate::config::HostDisplay;
    let row = "Display arrangement";
    // A pair the old rows could leave: shown as it streams, and left as it is until a pick.
    let saved = Settings { host_display: HostDisplay::Physical, virtual_display: true, ..Settings::default() };
    let mut f = settings_with(saved.clone());
    f.l.settings_view.section = "Display & image";
    f.l.settings_view.more = [true; 9];
    f.settle();
    assert_eq!(row_words(&f, row), ["Physical display", "Reset"], "Reset puts both keys back");
    assert!(!shows(&f, "PC render resolution"), "Physical display has no virtual display: {:?}", f.drawn);
    assert_eq!(f.l.settings, saved, "drawing it changes nothing");
    click_value(&mut f, row, "Physical display");
    for choice in DISPLAY_CHOICES {
        assert!(shows(&f, display_choice_name(choice)), "{choice:?} is offered: {:?}", f.drawn);
    }
    click_last(&mut f, "Virtual display");
    f.settle();
    assert_eq!((f.l.settings.host_display, f.l.settings.virtual_display), (HostDisplay::HostDefault, true));
    assert!(shows(&f, "PC render resolution"));
    click_value(&mut f, row, "Virtual display");
    click_last(&mut f, "Virtual display only");
    f.settle();
    assert_eq!((f.l.settings.host_display, f.l.settings.virtual_display), (HostDisplay::Exclusive, true));
    click_value(&mut f, row, "Virtual display only");
    click_last(&mut f, "The PC’s own setting");
    f.settle();
    assert_eq!((f.l.settings.host_display, f.l.settings.virtual_display), (HostDisplay::HostDefault, false));
    assert!(!shows(&f, "Virtual display"), "no row of its own: {:?}", f.drawn);
}

#[test]
fn a_pc_without_display_modes_offers_only_the_virtual_display_switch() {
    use crate::config::HostDisplay;
    let xml = "<?xml version=\"1.0\" encoding=\"utf-8\"?><root status_code=\"200\"><hostname>PC</hostname><uniqueid>uid</uniqueid><HttpsPort>47984</HttpsPort><ExternalPort>47989</ExternalPort><PairStatus>1</PairStatus><currentgame>0</currentgame><state>SUNSHINE_SERVER_FREE</state><Permission>1</Permission></root>";
    let info = nightjar_policy::http::ServerInfo::parse(xml, 47989).expect("the answer parses");
    // Settings ask for Exclusive, which this PC never receives: its page shows what it streams.
    let mut f = Frames::with(record("PC", true, false), Reach::Online, Some(info), Vec::new());
    f.size = vec2(1280.0, 12000.0);
    f.l.settings = Settings { host_display: HostDisplay::Exclusive, virtual_display: false, ..Settings::default() };
    f.l.settings_view.section = "Display & image";
    f.l.settings_view.more = [true; 9];
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    let row = "Display arrangement";
    let note = "This PC doesn’t take display arrangements: it streams as shown here, not as Virtual display only";
    assert_eq!(row_words(&f, row), ["The PC’s own setting"]);
    assert!(shows(&f, note), "the row says why: {:?}", f.drawn);
    click_value(&mut f, row, "The PC’s own setting");
    assert!(shows(&f, "Virtual display") && !shows(&f, "Virtual display only"), "two choices: {:?}", f.drawn);
    click_last(&mut f, "Virtual display");
    f.settle();
    assert_eq!(row_words(&f, row), ["Virtual display"]);
    assert!(!shows(&f, note), "a choice made here clears it");
}

#[test]
fn a_layer_holding_one_key_of_a_row_keeps_it_alone_until_a_value_is_chosen() {
    use crate::config::HostDisplay;
    // As the separate Virtual display row saved it: the PC's own switch, its arrangement inherited.
    let mut pc = record("PC", true, false);
    pc.settings = Some(serde_json::from_str(r#"{"virtual_display":true}"#).unwrap());
    let mut f = Frames::new(pc, Reach::Online);
    f.size = vec2(1280.0, 12000.0);
    f.l.settings = Settings { host_display: HostDisplay::Exclusive, ..Settings::default() };
    f.l.settings_view.section = "Display & image";
    f.l.settings_view.more = [true; 9];
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    f.settle();
    let row = "Display arrangement";
    assert!(row_words(&f, row).contains(&"Virtual display only"), "it streams the inherited arrangement: {:?}", row_words(&f, row));
    assert!(!f.l.edit_overrides.has(&["host_display"]), "drawing the page adds nothing to the PC's layer");
    assert!(f.l.edit_overrides.has(&["virtual_display"]));
    // A pick writes both keys into the PC's layer.
    click_value(&mut f, row, "Virtual display only");
    click_last(&mut f, "Virtual display");
    f.settle();
    assert!(f.l.edit_overrides.has(&["host_display"]) && f.l.edit_overrides.has(&["virtual_display"]));
    assert_eq!((f.l.edit_settings.host_display, f.l.edit_settings.virtual_display), (HostDisplay::HostDefault, true));
}


#[test]
fn an_arrangement_picked_on_a_pcs_page_leaves_its_virtual_display_switch_to_the_settings() {
    use crate::config::HostDisplay;
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 12000.0);
    f.l.settings_view.section = "Display & image";
    f.l.settings_view.more = [true; 9];
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    let row = "Display arrangement";
    // An arrangement: only host_display goes into the PC's settings, so Remote Monitor there keeps
    // following the main settings' switch, as before the merge.
    click_value(&mut f, row, "The PC’s own setting");
    click_last(&mut f, "Extend");
    f.settle();
    assert_eq!(f.l.edit_settings.host_display, HostDisplay::Extended);
    assert!(f.l.edit_overrides.has(&["host_display"]));
    assert!(!f.l.edit_overrides.has(&["virtual_display"]), "the switch stays the main settings'");
    // Virtual display and the PC's own setting set the switch too, as on Android.
    click_value(&mut f, row, "Extend");
    click_last(&mut f, "Virtual display");
    f.settle();
    assert!(f.l.edit_overrides.has(&["host_display"]) && f.l.edit_overrides.has(&["virtual_display"]));
    assert_eq!((f.l.edit_settings.host_display, f.l.edit_settings.virtual_display), (HostDisplay::HostDefault, true));
}

#[test]
fn frames_waiting_for_the_decoder_show_their_choice() {
    let mut s = Settings::default();
    assert_eq!(decode_queue_choice(s.decode_queue_frames), 15);
    for (saved, shown) in [(0, 15), (1, 3), (3, 3), (4, 5), (8, 8), (9, 15), (40, 15)] {
        assert_eq!(decode_queue_choice(saved), shown, "{saved}");
    }
    assert!(DECODE_QUEUE_CHOICES.iter().all(|&c| c == decode_queue_choice(c)));
    let row = ANDROID_KEYS.iter().find(|(desktop, _)| *desktop == "decode_queue_frames");
    assert_eq!(row, Some(&("decode_queue_frames", "list_decode_queue_frames")));
    s.decode_queue_frames = 5;
    assert_eq!(value_text("decode_queue_frames", &s, &crate::stream_types::Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: true }, crate::glyphs::Style::Xbox), "5 frames");
}

/// Whole frames under VRR shows where Variable refresh rate does, with its On: not where Windows
/// cannot present with tearing (the VRR row is hidden and streams use no VRR), nor with Fastest.
#[test]
fn whole_frames_under_vrr_shows_only_with_vrr_on_and_offered() {
    let drawn = |s: Settings, tearing: bool| {
        let mut f = settings_with(s);
        f.l.tearing_supported = tearing;
        open_folds(&mut f);
        f.l.settings_view.section = "Display & image";
        f.settle();
        setting_keys(&f).contains(&"vrr_wait")
    };
    assert_eq!(drawn(Settings::default(), true), cfg!(windows));
    assert!(!drawn(Settings::default(), false), "no tearing: no VRR on Windows, no scan-line wait elsewhere");
    assert!(!drawn(Settings { present_at_host_times: false, vrr_tear_free: false, ..Settings::default() }, true), "Fastest");
    assert!(!drawn(Settings { vrr: false, ..Settings::default() }, true), "VRR off");
}

#[test]
fn the_simple_layer_writes_and_reads_the_settings_it_stands_for() {
    let display = Display { width: 2560, height: 1440, refresh_hz: 120.0, refresh_reported: true };
    let caps = simple_caps(&display, false, true);
    let mut s = Settings::default();
    let recommended = simple::recommended(&caps);
    // Applying Auto after "Fastest" writes VRR's three keys together, so the Variable refresh rate
    // row reads On rather than Custom next to these rows.
    let mut from_fastest = Settings { present_at_host_times: false, vrr_tear_free: false, ..Settings::default() };
    simple_apply(&mut from_fastest, &display, &caps, recommended);
    assert_eq!((from_fastest.vrr, from_fastest.present_at_host_times, from_fastest.vrr_tear_free),
        (true, true, true), "Auto leaves the VRR row On, not Custom");
    simple_apply(&mut s, &display, &caps, recommended);
    assert_eq!((s.width, s.height, s.fps), (0, 0, 0), "the screen's own size and rate");
    assert_eq!(s.frame_pacing, FramePacing::Latency);
    assert!(s.vrr && s.present_at_host_times);
    assert_eq!(s.bitrate_kbps, 0, "the bitrate for the size and rate");
    assert!(!s.hdr);
    let (choices, custom) = simple_state(&s, &display, &caps, true);
    assert!(!custom);
    assert_eq!(choices, recommended);
    // One changed advanced setting reads as Custom.
    s.frame_pacing = FramePacing::CapFps;
    assert!(simple_state(&s, &display, &caps, true).1);
    // Save data lowers the bitrate and lets it fall when the link struggles, whatever "Never" was.
    s.lower_bitrate = LowerBitrate::Keep;
    simple_apply(&mut s, &display, &caps, simple::Choices { picture: simple::Picture::SaveData, ..recommended });
    assert!(s.lower_bitrate == LowerBitrate::WhenFramesLost && s.adaptive_quality == crate::config::QualityMode::Battery,
        "Save data is never overridden by an old Never");
    assert!(s.bitrate_kbps > 0 && s.bitrate_kbps < stream_mode(&Settings::default(), display).3);
    let (choices, custom) = simple_state(&s, &display, &caps, true);
    assert!(!custom && choices.picture == simple::Picture::SaveData);
    // A saved PyroWave codec does not make Best store PyroWave's own bitrate.
    let mut pyro = Settings { codec: CodecPreference::PyroWave, ..Settings::default() };
    simple_apply(&mut pyro, &display, &caps, simple::Choices { picture: simple::Picture::Best, ..recommended });
    assert_eq!(pyro.codec, CodecPreference::Auto);
    assert!(pyro.bitrate_kbps < 100_000, "Best uses the conventional codec's bitrate, not PyroWave's {}", pyro.bitrate_kbps);
    assert!(!simple_state(&pyro, &display, &caps, true).1);
}

#[test]
fn a_non_vrr_first_launch_reads_auto_not_custom() {
    let display = Display { width: 1920, height: 1080, refresh_hz: 60.0, refresh_reported: true };
    let caps = simple_caps(&display, false, false);
    let s = Settings::default();
    let (choices, custom) = simple_state(&s, &display, &caps, false);
    assert!(!custom, "a saved VRR the swap chain cannot tear is not Custom");
    assert_eq!(choices.smoothness, simple::Smoothness::Auto);
}

#[test]
fn the_hdr_row_value_reads_off_until_the_switch_is_on() {
    let display = Display { width: 1920, height: 1080, refresh_hz: 120.0, refresh_reported: true };
    let value = |s: &Settings| value_text("simple_hdr", s, &display, crate::glyphs::Style::Xbox);
    assert_eq!(value(&Settings::default()), "Off", "a fresh install streams SDR");
    assert_eq!(value(&Settings { hdr: true, ..Settings::default() }), "Auto (recommended)");
    assert_eq!(
        value(&Settings { simple_hdr: simple::Hdr::Auto, ..Settings::default() }),
        "Off",
        "the stored Auto does not make the row claim HDR while the switch is off"
    );
}

#[test]
fn a_fresh_hdr_screen_reads_hdr_off_like_the_stream() {    let display = Display { width: 1920, height: 1080, refresh_hz: 120.0, refresh_reported: true };
    let caps = simple_caps(&display, true, true);
    let s = Settings::default();
    assert!(!s.hdr, "the stream stays SDR: the HDR switch is at its app default");
    let (choices, custom) = simple_state(&s, &display, &caps, true);
    assert!(!custom, "the row shows what the stream does, not Custom");
    assert_eq!(choices.hdr, simple::Hdr::Off, "a fresh install streams SDR, so the row reads Off");
    // A pick in another row keeps HDR off (it writes the whole profile from the rows).
    let mut picked = s.clone();
    simple_apply(&mut picked, &display, &caps, simple::Choices { smoothness: simple::Smoothness::Balanced, ..choices });
    assert!(!picked.hdr, "a pick in another row must not turn HDR on");
    assert_eq!(simple_state(&picked, &display, &caps, true).0.hdr, simple::Hdr::Off);
    // The HDR row's own Auto turns it on, and a later pick elsewhere leaves it on.
    simple_apply(&mut picked, &display, &caps, simple::Choices { hdr: simple::Hdr::Auto, ..choices });
    assert!(picked.hdr);
    let (after, custom) = simple_state(&picked, &display, &caps, true);
    assert!(!custom);
    assert_eq!(after.hdr, simple::Hdr::Auto);
    simple_apply(&mut picked, &display, &caps, simple::Choices { picture: simple::Picture::Best, ..after });
    assert!(picked.hdr, "a pick in another row must not turn HDR off");
}

#[test]
fn a_custom_row_elsewhere_still_carries_the_streams_own_hdr() {
    // The rest of the settings match no profile, so the rows read Custom; their choices still carry
    // the stream's HDR state, so a pick in one row cannot flip it.
    let display = Display { width: 1920, height: 1080, refresh_hz: 120.0, refresh_reported: true };
    let caps = simple_caps(&display, true, true);
    let mut off = Settings { frame_pacing: FramePacing::CapFps, ..Settings::default() };
    let (choices, custom) = simple_state(&off, &display, &caps, true);
    assert!(custom);
    assert_eq!(choices.hdr, simple::Hdr::Off);
    simple_apply(&mut off, &display, &caps, simple::Choices { picture: simple::Picture::Best, ..choices });
    assert!(!off.hdr, "a pick in another row must not turn HDR on");
    let mut on = Settings { frame_pacing: FramePacing::CapFps, hdr: true, ..Settings::default() };
    let (choices, custom) = simple_state(&on, &display, &caps, true);
    assert!(custom);
    assert_eq!(choices.hdr, simple::Hdr::Auto);
    simple_apply(&mut on, &display, &caps, simple::Choices { picture: simple::Picture::Best, ..choices });
    assert!(on.hdr, "a pick in another row must not turn HDR off");
}

#[test]
fn a_hand_change_that_equals_another_profile_reads_as_custom() {
    let display = Display { width: 1920, height: 1080, refresh_hz: 120.0, refresh_reported: true };
    let caps = simple_caps(&display, false, true);
    let mut s = Settings::default();
    simple_apply(&mut s, &display, &caps, simple::recommended(&caps));
    // Auto writes the newest frame at Latency; Balanced's pacing now matches everything else Auto
    // writes, but the stored choice is still Auto, so the rows must not claim Balanced.
    s.frame_pacing = FramePacing::Balanced;
    assert!(simple_state(&s, &display, &caps, true).1, "a hand change equal to another profile is Custom");
}

#[test]
fn the_setup_section_shows_the_simple_layer_and_back_applies_it() {
    let mut f = settings_with(Settings::default());
    f.l.tearing_supported = true;
    f.l.settings_view.section = "Recommended";
    f.settle();
    for row in ["Stream smoothness", "Picture quality", "Screen", "HDR", "Console mode", "Back to recommended"] {
        assert!(shows(&f, row), "{row} is not drawn: {:?}", f.drawn);
    }
    assert!(!row_words(&f, "Stream smoothness").contains(&"Custom"), "{:?}", row_words(&f, "Stream smoothness"));
    f.l.settings.frame_pacing = FramePacing::CapFps;
    assert!(simple_state(&f.l.settings, &f.l.display, &simple_caps(&f.l.display, false, true), true).1);
    f.settle();
    assert!(row_words(&f, "Stream smoothness").contains(&"Custom"), "{:?}", row_words(&f, "Stream smoothness"));
    // Picking the stored choice while Custom applies the profile (it is what the row shows).
    let _ = click_value(&mut f, "Stream smoothness", "Custom");
    let _ = click_last(&mut f, "Auto (recommended)");
    f.settle();
    assert_eq!(FramePacing::Latency, f.l.settings.frame_pacing);
    f.l.settings.frame_pacing = FramePacing::CapFps;
    f.settle();
    let _ = click_text(&mut f, "Back to recommended");
    f.settle();
    assert_eq!(f.l.settings.frame_pacing, FramePacing::Latency);
    assert!(!row_words(&f, "Stream smoothness").contains(&"Custom"), "{:?}", row_words(&f, "Stream smoothness"));
}

/// How many rows the main settings show at their defaults with every fold open, per section listed
/// ("More settings" chosen): the number the settings-simplification work counts, 108 before rows
/// that can do nothing were left out. `--nocapture` prints them by section.
#[test]
fn row_counts_at_defaults() {
    let mut f = settings_with(Settings::default());
    f.size = vec2(1280.0, 20000.0);
    open_folds(&mut f);
    let listed: Vec<String> = rail_entries(&f).into_iter().map(str::to_owned).collect();
    let mut total = 0;
    for section in SETTINGS_SECTIONS {
        if !listed.iter().any(|l| l == section) {
            println!("{section:18} not listed");
            continue;
        }
        in_section(&mut f, section);
        let rows = f.l.settings_view.drawn.len();
        total += rows;
        println!("{section:18} {rows:3} {:?}", f.l.settings_view.drawn.iter().map(|(id, _)| *id).collect::<Vec<_>>());
    }
    println!("total {total}");
    assert_eq!(total, ROWS_AT_DEFAULTS, "rows shown at the defaults");
}

/// The main settings' rows at their defaults, every fold open, the new "Show debug settings" included.
/// Each platform lists only the rows its build can use, so the number is its own: macOS has no
/// "Group incoming video packets" and none of Linux' renderer, display output, video planes and
/// GPU power rows; Windows has rows of its own in their place.
const ROWS_AT_DEFAULTS: usize = if cfg!(windows) {
    98
} else if cfg!(target_os = "macos") {
    95
} else {
    100
};

/// Debug is listed only with "Show debug settings", while one of its settings is changed, while it
/// is open or while a search runs; its rows keep their values and search finds them.
#[test]
fn debug_is_listed_on_request_or_while_a_debug_setting_is_changed() {
    let mut f = settings_with(Settings::default());
    assert!(!rail_entries(&f).contains(&"Debug"), "not by default: {:?}", rail_entries(&f));
    f.l.settings.show_debug_settings = true;
    f.settle();
    assert_eq!(rail_entries(&f).last(), Some(&"Debug"), "with the switch, last");
    f.l.settings.show_debug_settings = false;
    f.l.settings.link_probe = true;
    f.settle();
    assert!(rail_entries(&f).contains(&"Debug"), "a changed Debug setting keeps it in sight");
    f.l.settings.link_probe = false;
    f.settle();
    assert!(!rail_entries(&f).contains(&"Debug"));
    // A search looks everywhere, Debug included.
    f.l.settings_view.query = "error correction".into();
    f.settle();
    assert!(shows(&f, "Error correction") && rail_entries(&f).contains(&"Debug"), "{:?}", f.drawn);
    // A link that leads into it keeps it listed while it shows.
    f.l.settings_view.query.clear();
    f.l.settings_view.section = "Debug";
    f.settle();
    assert!(rail_entries(&f).contains(&"Debug") && shows(&f, "Error correction"));
}

/// A PC's settings list Debug by the same rule, and while the PC sets one of its settings.
#[test]
fn a_pcs_settings_list_debug_by_the_same_rule() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    assert!(!rail_entries(&f).contains(&"Debug"), "{:?}", rail_entries(&f));
    f.l.edit_overrides = serde_json::from_str(r#"{"link_probe":true}"#).unwrap();
    f.settle();
    assert!(rail_entries(&f).contains(&"Debug"), "the PC sets one: {:?}", rail_entries(&f));
}

/// "Show debug settings" is an App row behind its fold, off by default, and its value is kept in
/// settings.json.
#[test]
fn show_debug_settings_is_an_app_row_and_its_value_is_kept() {
    assert!(!Settings::default().show_debug_settings);
    assert!(!main_row("show_debug_settings"), "behind App's fold");
    let saved = serde_json::to_string(&Settings { show_debug_settings: true, ..Settings::default() }).unwrap();
    let read: Settings = serde_json::from_str(&saved).unwrap();
    assert!(read.show_debug_settings, "read back");
    let older: Settings = serde_json::from_str("{}").unwrap();
    assert!(!older.show_debug_settings, "off for settings saved before it");
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    in_section(&mut f, "App");
    assert!(shows(&f, "Show debug settings"), "{:?}", f.drawn);
}

/// The rows a PC's, game's or profile's settings or the stream menu can make act are never left
/// out: while the main settings leave them nothing to do they say why, and they lose the note once
/// their mode comes on.
#[test]
fn rows_another_layer_can_make_act_stay_with_their_reason() {
    type Change = fn(&mut Settings);
    let rows: [(&str, &str, &str, Change); 5] = [
        ("Video", "Minimum resolution", "Not used without Automatic quality", |s| s.adaptive_quality = crate::config::QualityMode::Motion),
        ("Video", "Minimum frame rate", "Not used without Automatic quality", |s| s.adaptive_quality = crate::config::QualityMode::Motion),
        ("Display & image", "Upscaling sharpness", "Not used with Bilinear", |s| s.upscaling = crate::config::Upscaling::Fsr1),
        ("Streaming", "Overlay position", "Not used without Performance overlay", |s| s.overlay = true),
        ("Streaming", "Second screen resolution", "Not used without Dual screen mode", |s| s.dual_screen = true),
    ];
    for (section, label, reason, on) in rows {
        let base = Settings { upscaling: crate::config::Upscaling::Bilinear, disable_rumble: true, ..Settings::default() };
        let mut f = settings_with(base);
        open_folds(&mut f);
        in_section(&mut f, section);
        assert!(shows(&f, label) && shows(&f, reason), "{label} stays, saying why: {:?}", f.drawn);
        on(&mut f.l.settings);
        f.settle();
        assert!(shows(&f, label) && !shows(&f, reason), "{label} acts now: {:?}", f.drawn);
    }
}

/// The IGDB client secret does nothing until the client ID is set: it is left out until then, comes
/// back on the next frame, stays with a secret of its own saying why, and search finds it.
#[test]
fn the_igdb_secret_is_left_out_until_the_client_id_is_set() {
    let (label, reason) = ("IGDB client secret", "Not used without IGDB client ID");
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    in_section(&mut f, "Advanced");
    assert!(!shows(&f, label), "{:?}", f.drawn);
    f.l.game_info_keys.igdb_client_id = "id".into();
    f.settle();
    assert!(shows(&f, label), "{:?}", f.drawn);
    let mut f = settings_with(Settings::default());
    f.l.game_info_keys.igdb_client_secret = "secret".into();
    open_folds(&mut f);
    in_section(&mut f, "Advanced");
    assert!(shows(&f, label) && shows(&f, reason), "{:?}", f.drawn);
    let mut f = settings_with(Settings::default());
    f.l.settings_view.query = label.to_lowercase();
    f.settle();
    assert!(shows(&f, label) && shows(&f, reason), "search finds it: {:?}", f.drawn);
}

/// A row shown only because it holds a value stays while its own control is in use: clearing the
/// IGDB client secret (client ID empty) keeps the row under the caret until the field loses focus.
#[test]
fn a_row_set_back_to_its_default_stays_while_its_field_has_focus() {
    use egui::accesskit::Role;
    let label = "IGDB client secret";
    let mut f = settings_with(Settings::default());
    f.ctx.enable_accesskit();
    f.l.game_info_keys.igdb_client_secret = "secret".into();
    open_folds(&mut f);
    in_section(&mut f, "Advanced");
    assert!(shows(&f, label), "held by its secret: {:?}", f.drawn);
    let field = f.access.as_ref().expect("AccessKit is on").nodes.iter()
        .find(|(_, n)| n.role() == Role::TextInput && n.label() == Some(label))
        .map(|(_, n)| n.bounds().expect("laid out"))
        .expect("the secret's field");
    f.click_at(Pos2::new(((field.x0 + field.x1) / 2.0) as f32, ((field.y0 + field.y1) / 2.0) as f32), egui::PointerButton::Primary);
    assert!(f.ctx.text_edit_focused(), "the field has focus");
    // Cleared while it keeps focus: the row stays, the caret with it.
    for _ in 0..12 {
        f.press(egui::Key::Backspace);
    }
    assert!(f.l.game_info_keys.igdb_client_secret.is_empty(), "{:?}", f.l.game_info_keys.igdb_client_secret);
    assert!(f.ctx.text_edit_focused() && shows(&f, label), "kept while in use: {:?}", f.drawn);
    // Focus leaves: the row does nothing without the client ID and holds nothing, so it goes.
    f.press(egui::Key::Escape);
    f.settle();
    assert!(!f.ctx.text_edit_focused());
    assert!(!shows(&f, label), "gone once focus left: {:?}", f.drawn);
}
/// Vibration is one list over the two stored fields: Off keeps the feel, a feel turns vibration on.
#[test]
fn vibration_is_one_choice_over_the_two_stored_fields() {
    use nightjar_policy::rumble::RumbleFeel;
    let mut s = Settings::default();
    assert_eq!(vibration_choice(&s), Vibration::Feel(RumbleFeel::Standard));
    set_vibration_choice(&mut s, Vibration::Feel(RumbleFeel::Strong));
    assert!(!s.disable_rumble && s.rumble_feel == RumbleFeel::Strong);
    set_vibration_choice(&mut s, Vibration::Off);
    assert!(s.disable_rumble && s.rumble_feel == RumbleFeel::Strong, "Off keeps the feel");
    assert_eq!(vibration_choice(&s), Vibration::Off);
    // A settings file that had vibration off with any feel reads Off; on, the feel.
    let old: Settings = serde_json::from_str(r#"{"disable_rumble":true,"rumble_feel":"subtle"}"#).unwrap();
    assert_eq!(vibration_choice(&old), Vibration::Off);
    set_vibration_choice(&mut s, Vibration::Feel(RumbleFeel::Subtle));
    assert!(!s.disable_rumble);
    assert_eq!(VIBRATION_CHOICES.map(vibration_name), ["Off", "Subtle", "Standard", "Strong"]);
    // One row for both keys, in Controls, and the old rows are gone.
    let mut f = settings_with(Settings::default());
    open_folds(&mut f);
    in_section(&mut f, "Controls");
    assert!(shows(&f, "Vibration") && !shows(&f, "Turn off vibration") && !shows(&f, "Rumble feel"), "{:?}", f.drawn);
}
