//! The settings page as people use it: sections, search, reset to default and the PC and game
//! layers, driven through whole frames like the launcher's other tests.

use super::*;

/// Main settings at their defaults but `changed`, on a tall window where a section never scrolls.
fn settings_with(changed: Settings) -> Frames {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.l.settings = changed;
    f.l.go(Screen::Settings);
    f.settle();
    f
}

/// Presses `key` until focus lands on the widget drawn with `text`. False when it never does.
fn focus_to(f: &mut Frames, key: Key, text: &str) -> bool {
    for _ in 0..16 {
        f.press(key);
        let focused = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
        let at = f.placed.iter().find(|(t, ..)| t == text).map(|p| p.1.center());
        if focused.zip(at).is_some_and(|(r, at)| r.contains(at)) {
            return true;
        }
    }
    false
}

#[test]
fn the_d_pad_scrolls_each_row_it_reaches_into_view() {
    // The main settings and a PC's own, on a window too short for a section.
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    let id = f.hosts.list[0].id;
    for page in [Screen::Settings, Screen::HostSettings(id)] {
        if page == Screen::Settings { f.l.go(page) } else { f.l.open_host_settings(id, &f.hosts.list[0]) }
        f.settle();
        let first = f.ctx.memory(|m| m.focused()).expect("the first row has focus");
        let top = f.ctx.read_response(first).expect("laid out").rect.top();
        let mut reached = std::collections::HashSet::new();
        for _ in 0..24 {
            f.press(Key::ArrowDown);
            // The scroll animates.
            f.settle();
            f.settle();
            let focused = f.ctx.memory(|m| m.focused()).expect("a row has focus");
            reached.insert(focused);
            let rect = f.ctx.read_response(focused).expect("laid out").rect;
            assert!(rect.top() >= 0.0 && rect.bottom() <= f.size.y, "{page:?}: the focused row {rect:?} is in the window");
        }
        let moved = top - f.ctx.read_response(first).expect("laid out").rect.top();
        assert!(reached.len() > 6 && moved > 100.0, "{page:?}: Down went past the window's edge and the page scrolled ({} rows, {moved})", reached.len());
    }
}

#[test]
fn the_search_finds_rows_in_every_section_and_says_when_nothing_matches() {
    let mut f = settings_with(Settings::default());
    f.l.settings_view.query = "microphone".into();
    f.settle();
    for text in ["Results for “microphone”", "Audio", "Microphone", "Echo cancellation", "Controls", "DualSense USB audio"] {
        assert!(shows(&f, text), "{text} is found: {:?}", f.drawn);
    }
    assert!(!shows(&f, "Resolution") && !shows(&f, "Channels"), "rows without the word hide: {:?}", f.drawn);
    // A section's name finds its rows; every word must match somewhere.
    f.l.settings_view.query = "audio channels".into();
    f.settle();
    assert!(shows(&f, "Channels") && !shows(&f, "Microphone"), "{:?}", f.drawn);
    f.l.settings_view.query = "zzzz".into();
    f.settle();
    assert!(shows(&f, "No settings match “zzzz”") && !shows(&f, "Channels"), "{:?}", f.drawn);
    click_text(&mut f, "Clear search");
    assert!(f.l.settings_view.query.is_empty() && shows(&f, "Resolution"), "Clear search goes back to the section: {:?}", f.drawn);
}

/// The Ctrl+F hint shows only where the empty field's placeholder leaves it room, in every language.
#[test]
fn the_ctrl_f_hint_never_covers_the_search_placeholder() {
    let mut f = settings_with(Settings::default());
    for language in crate::config::Language::ALL.into_iter().filter(|l| *l != crate::config::Language::System) {
        f.l.settings.language = language;
        for width in [640.0, 800.0, 1024.0, 1280.0] {
            f.size = vec2(width, 4000.0);
            f.settle();
            let at = |text: &str| f.placed.iter().find(|(t, ..)| t == text).map(|(_, r, _)| *r);
            let hint = at(tr("Search settings")).unwrap_or_else(|| panic!("{language:?} at {width}: no placeholder in {:?}", f.drawn));
            if let Some(key) = at(tr("Ctrl+F")) {
                assert!(hint.right() + 8.0 <= key.left(), "{language:?} at {width}: the placeholder {hint:?} runs into Ctrl+F {key:?}");
            }
        }
    }
}

#[test]
fn ctrl_f_reaches_the_search_and_escape_leaves_it_before_the_page() {
    let mut f = settings_with(Settings::default());
    f.press_with(Key::F, egui::Modifiers::COMMAND);
    assert_eq!(f.ctx.memory(|m| m.focused()), Some(Id::unique("settings-search")), "Ctrl+F focuses the search");
    f.frame(vec![egui::Event::Text("hdr".into())]);
    f.settle();
    assert_eq!(f.l.settings_view.query, "hdr");
    assert!(shows(&f, "HDR") && !shows(&f, "Resolution"), "{:?}", f.drawn);
    // Escape leaves the field, then clears the search, then leaves the page.
    let commands = f.press(Key::Escape);
    assert!(f.l.screen == Screen::Settings && commands.is_empty() && f.l.settings_view.query == "hdr", "{}", commands.len());
    let commands = f.press(Key::Escape);
    assert!(f.l.screen == Screen::Settings && commands.is_empty() && f.l.settings_view.query.is_empty(), "{}", commands.len());
    let commands = f.press(Key::Escape);
    assert!(commands.iter().any(|c| matches!(c, Command::SaveSettings)), "then Escape leaves");
}

#[test]
fn a_section_opens_from_its_entry_by_mouse_and_by_controller() {
    let mut f = settings_with(Settings::default());
    assert!(shows(&f, "Resolution") && !shows(&f, "Channels"));
    click_text(&mut f, "Audio");
    assert!(shows(&f, "Channels") && !shows(&f, "Resolution"), "{:?}", f.drawn);
    // The D-pad: from the rows left to the list, down to Streaming, A opens it.
    f.l.go(Screen::Settings);
    f.l.settings_view.section = "Video";
    f.settle();
    f.press(Key::ArrowLeft);
    let focused = f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
    let rows = f.placed.iter().find(|(t, ..)| t == "Resolution").expect("the first row").1;
    assert!(focused.is_some_and(|r| r.right() < rows.left()), "Left reaches the search or the list: {focused:?}");
    assert!(focus_to(&mut f, Key::ArrowDown, "Streaming"), "Down walks the list");
    f.press(Key::Enter);
    assert!(shows(&f, "Recover interrupted streams") && !shows(&f, "Resolution"), "{:?}", f.drawn);
    // Opening a section during a search ends the search.
    f.l.settings_view.query = "bitrate".into();
    f.settle();
    click_text(&mut f, "Audio");
    assert!(f.l.settings_view.query.is_empty() && shows(&f, "Channels"), "{:?}", f.drawn);
}

#[test]
fn a_changed_row_offers_its_default_back() {
    let mut f = settings_with(Settings { fps: 120, ..Settings::default() });
    let resets: Vec<_> = f.placed.iter().filter(|(t, ..)| t == "Reset").collect();
    assert_eq!(resets.len(), 1, "only the changed row has a Reset: {:?}", f.drawn);
    click_text(&mut f, "Reset");
    assert_eq!(f.l.settings.fps, Settings::default().fps);
    assert!(!shows(&f, "Reset") && f.l.settings_dirty, "{:?}", f.drawn);
}

#[test]
fn reset_all_asks_first_and_keeps_the_language() {
    let mut f = settings_with(Settings { fps: 120, hdr: true, language: Language::English, ..Settings::default() });
    click_text(&mut f, "App");
    click_text(&mut f, "Reset all…");
    assert!(shows(&f, "Reset all settings?") && shows(&f, "Cancel"), "{:?}", f.drawn);
    // The question says what stays.
    assert!(shows(&f, "Every setting on this page goes back to its default. Your language, PCs, games, profiles, PIN lock, controller buttons, shortcuts and anything not on this page stay as they are. This can't be undone."), "{:?}", f.drawn);
    click_text(&mut f, "Cancel");
    assert!(f.l.settings.fps == 120 && shows(&f, "Reset all…"), "Cancel changes nothing");
    click_text(&mut f, "Reset all…");
    click_text(&mut f, "Reset all");
    assert_eq!((f.l.settings.fps, f.l.settings.hdr, f.l.settings.language), (0, false, Language::English));
    assert!(f.l.settings_dirty && shows(&f, "Reset all…"), "{:?}", f.drawn);
}

#[test]
fn a_pcs_row_says_whether_it_follows_the_main_settings_or_its_own() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.hosts.list[0].record.settings = Some(serde_json::from_str(r#"{"fps":120}"#).unwrap());
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    let main_fps = value_text("fps", &f.l.inherited_settings, &f.l.display, crate::glyphs::Style::Xbox);
    for text in ["1 setting is set here.", "From main settings", "For this PC", &format!("Otherwise {main_fps}")] {
        assert!(shows(&f, text), "{text}: {:?}", f.drawn);
    }
    assert!(!shows(&f, "Reset"), "a PC's rows go back with their box, not Reset");
    // Unticking the Frame rate's box gives the row back to the main settings.
    let menu = f.placed.iter().find(|(t, ..)| t == "120 fps").expect("the PC's own rate").1;
    let boxed = f.placed.iter().filter(|(t, r, _)| t == "For this PC" && r.top() >= menu.bottom()).map(|p| p.1)
        .min_by(|a, b| a.top().total_cmp(&b.top())).expect("its box");
    f.click_at(boxed.center(), egui::PointerButton::Primary);
    assert!(f.l.edit_overrides.is_empty() && shows(&f, "Nothing is set here yet."), "{:?}", f.drawn);
    // A PC's settings have no App section.
    assert!(!shows(&f, "App"), "{:?}", f.drawn);
}

/// A search set from code (or typed) after scrolling a long section shows its heading, not the
/// section's old scroll position.
#[test]
fn a_new_search_shows_its_results_from_the_top() {
    let mut f = settings_with(Settings::default());
    f.size = vec2(1280.0, 720.0);
    f.l.settings_view.section = "Controls";
    f.settle();
    f.scroll_to_end();
    let seen = |f: &Frames, text: &str| f.placed.iter().find(|(t, ..)| t == text).is_some_and(|(_, r, clip)| r.top() >= clip.top());
    assert!(!seen(&f, "Mouse, keyboard and controllers."), "the section scrolled: {:?}", f.drawn);
    f.l.settings_view.query = "bitrate".into();
    f.settle();
    assert!(seen(&f, "Results for “bitrate”"), "the results start at the top: {:?}", f.placed.iter().find(|(t, ..)| t.starts_with("Results")));
}

/// While searching, a section without matches shows no count, and its entry still opens it.
#[test]
fn a_section_without_matches_shows_no_count_and_opens_from_its_entry() {
    let mut f = settings_with(Settings::default());
    f.l.settings_view.query = "bitrate".into();
    f.settle();
    let found = f.l.settings_view.found;
    assert!(found.contains(&0) && found.iter().any(|&n| n > 0), "{found:?}");
    let rows_left = f.placed.iter().find(|(t, ..)| t == "Results for “bitrate”").expect("the results").1.left();
    let counts: Vec<&str> = f.placed.iter().filter(|(t, r, _)| r.right() < rows_left && t.parse::<usize>().is_ok()).map(|(t, ..)| t.as_str()).collect();
    let expected: Vec<String> = found.iter().filter(|&&n| n > 0).map(usize::to_string).collect();
    assert_eq!(counts, expected, "only the sections with matches show a count");
    click_text(&mut f, "Display & image");
    assert!(f.l.settings_view.query.is_empty() && f.l.settings_view.section == "Display & image", "{:?}", f.l.settings_view.query);
    assert!(shows(&f, "Video size") && !shows(&f, "Automatic bitrate"), "{:?}", f.drawn);
}

/// A PC's row box: unticked and ticked as a checkbox for readers, named for its row, and a click
/// keeps the row's value for the PC.
#[test]
fn a_pcs_row_box_is_a_named_checkbox_that_toggles() {
    use egui::accesskit::{Role, Toggled};
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.ctx.enable_accesskit();
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    let frame_rate_box = |f: &Frames| {
        f.access.as_ref().expect("AccessKit is on").nodes.iter()
            .find(|(_, n)| n.role() == Role::CheckBox && n.label() == Some("For this PC") && n.description() == Some("Frame rate"))
            .map(|(_, n)| (n.toggled(), n.bounds().expect("laid out")))
            .expect("the Frame rate row's box")
    };
    let (toggled, bounds) = frame_rate_box(&f);
    assert_eq!(toggled, Some(Toggled::False));
    assert!(bounds.y1 - bounds.y0 >= 24.0, "a box easy to hit: {bounds:?}");
    let at = Pos2::new(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32);
    f.click_at(at, egui::PointerButton::Primary);
    assert!(f.l.edit_overrides.has(&["fps"]), "the click keeps Frame rate for this PC");
    assert_eq!(frame_rate_box(&f).0, Some(Toggled::True));
    f.click_at(at, egui::PointerButton::Primary);
    assert!(f.l.edit_overrides.is_empty() && frame_rate_box(&f).0 == Some(Toggled::False), "a second click gives it back");
}

/// A PC's row box sits on the row's last line, beside where the value comes from, under a short
/// switch as under a menu.
#[test]
fn a_pcs_row_box_lines_up_with_where_the_value_comes_from() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    for row in ["Resolution", "Automatic bitrate", "Lower bitrate when the network is busy"] {
        let top = f.placed.iter().find(|(t, ..)| t == row).expect("the row").1.top();
        let below = |text: &str| f.placed.iter().filter(|(t, r, _)| t == text && r.top() > top).map(|p| p.1)
            .min_by(|a, b| a.top().total_cmp(&b.top())).unwrap_or_else(|| panic!("{text} under {row}"));
        let (state, boxed) = (below("From main settings"), below("For this PC"));
        assert!((state.center().y - boxed.center().y).abs() < 3.0, "{row}: {state:?} beside {boxed:?}");
    }
}

/// Left from a row's control lands on the open section's entry (or chip), not on the widget
/// egui finds nearest to the left: on a wide window that is the search or the top of the list,
/// on a narrow one the chip nearest the rows.
#[test]
fn left_from_the_rows_goes_to_the_open_sections_entry() {
    for (width, section) in [(1280.0, "Advanced"), (1280.0, "Audio"), (900.0, "Video"), (900.0, "Audio")] {
        let mut f = settings_with(Settings::default());
        f.size = vec2(width, 4000.0);
        f.l.go(Screen::Settings);
        f.l.settings_view.section = section;
        f.settle();
        let focused = |f: &Frames| f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
        let first = focused(&f).expect("the first row's control has focus");
        f.press(Key::ArrowLeft);
        // The entry or chip, in smaller type than the section's heading over the rows.
        let entry = f.placed.iter().filter(|(t, ..)| t == section).map(|p| p.1)
            .min_by(|a, b| a.height().total_cmp(&b.height())).expect("the entry");
        assert!(entry.right() < first.left() || entry.bottom() < first.top(), "{width}: {entry:?} is the entry");
        assert!(focused(&f).is_some_and(|r| r.contains(entry.center())), "{width} {section}: Left lands on {:?}, not the entry {entry:?}", focused(&f));
        if width < 940.0 {
            continue;
        }
        // Right from the list goes back into the rows.
        f.press(Key::ArrowRight);
        assert!(focused(&f).is_some_and(|r| r.left() > entry.right()), "{width} {section}: Right leaves the entry: {:?}", focused(&f));
    }
}

/// The profile notice's line sits beside its button's middle, not at the button's top.
#[test]
fn the_profile_notice_lines_up_with_its_button() {
    let mut f = settings_with(Settings::default());
    let travel = f.l.profiles.create("Travel", serde_json::from_str(r#"{"bitrate_kbps":20000}"#).unwrap()).unwrap();
    f.l.profiles.active = Some(travel);
    f.settle();
    let at = |text: &str| f.placed.iter().find(|(t, ..)| t.starts_with(text)).unwrap_or_else(|| panic!("{text}: {:?}", f.drawn)).1.center().y;
    let (line, button) = (at("Streams on this network use the profile Travel."), at("Manage profiles"));
    assert!((line - button).abs() < 2.0, "the notice at {line}, its button at {button}");
}

/// SECTION_KEYS lists every setting row in the order drawn, so a PC's count and Reset all miss none.
#[test]
fn the_section_key_table_is_what_the_page_draws() {
    let mut f = settings_with(Settings {
        bitrate_kbps: 30_000,
        adaptive_quality: crate::config::QualityMode::Motion,
        upscaling: crate::config::Upscaling::Fsr1,
        controller_mouse: true,
        client_roaming: true,
        ..Settings::default()
    });
    let only_here = |key: &str| match key {
        "native_touchpad" => cfg!(windows),
        "check_updates" => crate::self_update::current().checks(),
        "network_priority" => cfg!(any(target_os = "linux", windows)),
        "hdr_desktop_brightness" => cfg!(target_os = "linux"),
        "renderer" | "vulkan_video_first" | "vulkan_rfi" => cfg!(target_os = "linux"),
        _ => true,
    };
    for (at, section) in SETTINGS_SECTIONS.iter().enumerate() {
        f.l.settings_view.section = section;
        f.settle();
        let mut drawn: Vec<&str> = f.l.settings_view.drawn.iter().map(|(key, _)| *key).collect();
        drawn.dedup();
        let listed: Vec<&str> = SECTION_KEYS[at].iter().map(|keys| keys[0]).filter(|k| only_here(k)).collect();
        assert_eq!(drawn, listed, "{section}");
    }
}

/// The rarely needed settings sit in Debug, the last section, and in no other, on the main
/// settings and a PC's own; the search still finds them there. Align video to top stays in
/// Display & image.
#[test]
fn the_rarely_needed_settings_are_in_the_debug_section_only() {
    // Android's main rows in its order, then the desktop's own in Fine-tuning.
    const DEBUG: [&str; 13] = ["error_correction", "client_hint_bitrate", "tell_host_wireless", "tell_host_link_speed",
        "network_priority", "unbatched_input", "full_range", "decoder", "software_slices", "vulkan_video_first", "vulkan_rfi",
        "renderer", "cursor_shapes"];
    assert_eq!(SETTINGS_SECTIONS.last(), Some(&"Debug"));
    // Prioritize stream traffic does something on Linux and Windows, so only they draw it; Vulkan
    // video is Linux's; the renderer is Linux's choice for the whole app, so a PC's settings do not have it.
    let wanted = |screen: Screen| -> Vec<&str> {
        DEBUG.into_iter()
            .filter(|k| *k != "network_priority" || cfg!(any(target_os = "linux", windows)))
            .filter(|k| !matches!(*k, "vulkan_video_first" | "vulkan_rfi") || cfg!(target_os = "linux"))
            .filter(|k| *k != "renderer" || (cfg!(target_os = "linux") && screen == Screen::Settings))
            .collect()
    };
    let mut f = settings_with(Settings::default());
    let id = f.hosts.list[0].id;
    for screen in [Screen::Settings, Screen::HostSettings(id)] {
        f.l.go(screen);
        for section in SETTINGS_SECTIONS {
            if screen != Screen::Settings && section == "App" { continue; }
            f.l.settings_view.section = section;
            f.settle();
            let drawn: Vec<&str> = f.l.settings_view.drawn.iter().map(|(key, _)| *key).collect();
            if section == "Debug" {
                assert_eq!(drawn, wanted(screen), "{screen:?}: Debug holds exactly these");
            } else {
                assert_eq!(drawn.contains(&"align_video_top"), section == "Display & image", "{screen:?} {section}: {drawn:?}");
                assert!(!drawn.iter().any(|k| DEBUG.contains(k)), "{screen:?} {section}: {drawn:?}");
            }
        }
    }
    f.l.go(Screen::Settings);
    for (query, row) in [("full color range", "Full color range"), ("error correction", "Error correction"), ("local cursor", "Local cursor")] {
        f.l.settings_view.query = query.into();
        f.settle();
        // The sidebar always shows "Debug"; the section's count says the row was found there.
        assert!(shows(&f, row) && f.l.settings_view.found[SETTINGS_SECTIONS.len() - 1] > 0,
            "{query} finds {row} in Debug: {:?} {:?}", f.l.settings_view.found, f.drawn);
    }
}

/// Main settings like `settings_with`, with the AccessKit tree on to read which rows are disabled.
fn settings_read_with(changed: Settings) -> Frames {
    let mut f = settings_with(changed);
    f.ctx.enable_accesskit();
    f.settle();
    f
}

/// Whether the control whose description starts with `description` is disabled; None when no
/// such control is drawn.
fn control_disabled(f: &Frames, description: &str) -> Option<bool> {
    f.access.as_ref().expect("AccessKit is on").nodes.iter()
        .find(|(_, n)| n.role() != egui::accesskit::Role::Label && n.description().is_some_and(|d| d.starts_with(description)))
        .map(|(_, n)| n.is_disabled())
}

/// Frame pacing and V-Sync work with Variable refresh rate off and Show frames with it on: a
/// gate over some rows never carries over to the rows after them.
#[test]
fn the_vrr_rows_follow_their_own_gates() {
    let (pacing, vsync, host_times) = ("Without VRR: Lowest latency", "Without VRR: waits", "With VRR: each frame");
    for vrr in [false, true] {
        let mut f = settings_read_with(Settings { vrr, frame_pacing: crate::config::FramePacing::Latency, ..Settings::default() });
        f.l.settings_view.section = "Display & image";
        f.settle();
        assert_eq!(control_disabled(&f, pacing), Some(vrr), "vrr {vrr}: Frame pacing");
        assert_eq!(control_disabled(&f, vsync), Some(vrr), "vrr {vrr}: V-Sync");
        assert_eq!(control_disabled(&f, host_times), Some(!vrr), "vrr {vrr}: Show frames at the PC's frame times");
        let notes = |text: &str| f.drawn.iter().filter(|t| *t == text).count();
        assert_eq!(notes("Used only with Variable refresh rate off."), if vrr { 2 } else { 0 }, "{:?}", f.drawn);
        assert_eq!(notes("Turn on Variable refresh rate to use this."), usize::from(!vrr), "{:?}", f.drawn);
    }
    // A frame pacing that waits for the refresh keeps V-Sync on, and says so.
    let mut f = settings_read_with(Settings { vrr: false, frame_pacing: crate::config::FramePacing::Balanced, ..Settings::default() });
    f.l.settings_view.section = "Display & image";
    f.settle();
    assert_eq!((control_disabled(&f, pacing), control_disabled(&f, vsync)), (Some(false), Some(true)));
    assert!(shows(&f, "Always on with the chosen frame pacing."), "{:?}", f.drawn);
}

/// Full chroma turns off YUV 4:4:4 and 10-bit SDR, not Video decoding (in Debug): only PyroWave does.
#[test]
fn full_chroma_leaves_video_decoding_alone() {
    let decoding = "Automatic decodes on the graphics card";
    let in_debug = |f: &mut Frames| {
        f.l.settings_view.section = "Debug";
        f.settle();
    };
    let mut f = settings_read_with(Settings { experimental_packed_444: true, ..Settings::default() });
    assert_eq!(control_disabled(&f, "Keeps full color detail"), Some(true));
    in_debug(&mut f);
    assert_eq!(control_disabled(&f, decoding), Some(false));
    let mut f = settings_read_with(Settings { codec: CodecPreference::PyroWave, ..Settings::default() });
    in_debug(&mut f);
    assert_eq!(control_disabled(&f, decoding), Some(true));
}

/// A PC's own value on a row that is turned off (YUV 4:4:4 under PyroWave) can still be given
/// back with its box; a row turned off and not set here keeps its box off too.
#[test]
fn a_pcs_own_value_on_a_disabled_row_can_be_cleared() {
    use egui::accesskit::{Role, Toggled};
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.ctx.enable_accesskit();
    f.hosts.list[0].record.settings = Some(serde_json::from_str(r#"{"codec":"PyroWave","prefer_yuv444":true}"#).unwrap());
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    let row_box = |f: &Frames, row: &str| {
        f.access.as_ref().expect("AccessKit is on").nodes.iter()
            .find(|(_, n)| n.role() == Role::CheckBox && n.label() == Some("For this PC") && n.description() == Some(row))
            .map(|(_, n)| (n.toggled(), n.is_disabled(), n.bounds().expect("laid out")))
            .unwrap_or_else(|| panic!("{row}'s box"))
    };
    let (toggled, disabled, bounds) = row_box(&f, "Sharper colored text (YUV 4:4:4)");
    assert_eq!((toggled, disabled), (Some(Toggled::True), false), "the PC's own value can be unticked");
    assert_eq!(control_disabled(&f, "Keeps full color detail"), Some(true), "the switch itself stays off under PyroWave");
    let (toggled, disabled, _) = row_box(&f, "Less color banding (10-bit SDR)");
    assert_eq!((toggled, disabled), (Some(Toggled::False), true), "a row off and not set here keeps its box off");
    f.click_at(Pos2::new(((bounds.x0 + bounds.x1) / 2.0) as f32, ((bounds.y0 + bounds.y1) / 2.0) as f32), egui::PointerButton::Primary);
    assert!(!f.l.edit_overrides.has(&["prefer_yuv444"]) && f.l.edit_overrides.has(&["codec"]), "{:?}", f.l.edit_overrides);
    assert_eq!(row_box(&f, "Sharper colored text (YUV 4:4:4)").0, Some(Toggled::False));
}

/// Left from any row stays in that row (its Reset, a slider) or goes to the open section's
/// entry, never to a Reset or control of another row, with changed rows above and below.
#[test]
fn left_never_lands_on_another_rows_reset() {
    let focused = |f: &Frames| f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
    let changed = [Settings { fps: 120, ..Settings::default() }, Settings { fps: 60, bitrate_kbps: 40_000, ..Settings::default() }];
    for width in [1280.0, 900.0] {
        for (case, settings) in changed.iter().enumerate() {
            for step in 0..8 {
                let mut f = settings_with(settings.clone());
                f.size = vec2(width, 4000.0);
                f.l.go(Screen::Settings);
                f.l.settings_view.section = "Video";
                f.settle();
                for _ in 0..step {
                    f.press(Key::ArrowDown);
                }
                let from = focused(&f).expect("a row's control has focus");
                let entry = f.placed.iter().filter(|(t, ..)| t == "Video").map(|p| p.1)
                    .min_by(|a, b| a.height().total_cmp(&b.height())).expect("the entry");
                if from.contains(entry.center()) {
                    continue;
                }
                f.press(Key::ArrowLeft);
                let to = focused(&f).expect("Left keeps a focus");
                let same_row = to.top() < from.bottom() && from.top() < to.bottom();
                assert!(to.contains(entry.center()) || same_row, "{width} case {case} step {step}: Left from {from:?} lands on {to:?}, not the entry {entry:?} or its row");
            }
        }
    }
    // A changed row's own Reset is still left of its control.
    let mut f = settings_with(Settings { width: 1280, height: 720, ..Settings::default() });
    let from = focused(&f).expect("Resolution has focus");
    f.press(Key::ArrowLeft);
    let reset = f.placed.iter().find(|(t, ..)| t == "Reset").expect("Resolution's Reset").1;
    assert!(focused(&f).is_some_and(|r| r.contains(reset.center())), "Left from {from:?} reaches the row's Reset {reset:?}, not {:?}", focused(&f));
}

/// Escape (B) in the search field leaves it for the first row found, so the D-pad carries on.
#[test]
fn escape_in_the_search_moves_focus_to_the_rows() {
    let mut f = settings_with(Settings::default());
    f.press_with(Key::F, egui::Modifiers::COMMAND);
    f.frame(vec![egui::Event::Text("hdr".into())]);
    f.settle();
    f.press(Key::Escape);
    let focused = |f: &Frames| f.ctx.memory(|m| m.focused()).and_then(|id| f.ctx.read_response(id)).map(|r| r.rect);
    let results = f.placed.iter().find(|(t, ..)| t == "Results for “hdr”").expect("the results").1;
    assert_eq!(f.l.settings_view.query, "hdr", "the first Escape keeps the search");
    assert!(focused(&f).is_some_and(|r| r.left() > results.left() && r.top() > results.bottom()), "a row found has focus: {:?}", focused(&f));
    // With nothing found, the open section's entry takes it.
    f.press_with(Key::F, egui::Modifiers::COMMAND);
    f.frame(vec![egui::Event::Text("zzzz".into())]);
    f.settle();
    f.press(Key::Escape);
    let entry = f.placed.iter().filter(|(t, ..)| t == "Video").map(|p| p.1).min_by(|a, b| a.height().total_cmp(&b.height())).expect("the entry");
    assert!(focused(&f).is_some_and(|r| r.contains(entry.center())), "the entry has focus: {:?}", focused(&f));
}

/// Ctrl+F is the page's: with a dialog open it leaves the focus where it is.
#[test]
fn ctrl_f_leaves_an_open_dialog_alone() {
    let mut f = settings_with(Settings::default());
    f.l.ask(Dialog::Message { title: "Network test failed".into(), body: "The PC did not answer.".into() });
    f.settle();
    let before = f.ctx.memory(|m| m.focused());
    assert!(before.is_some(), "the dialog's button has focus");
    f.press_with(Key::F, egui::Modifiers::COMMAND);
    assert_eq!(f.ctx.memory(|m| m.focused()), before, "the dialog keeps the focus");
}

/// A PC's own value without a row on the page (beam racing) still counts, and Clear all clears it.
#[test]
fn a_pcs_value_without_a_row_counts_and_clears() {
    let mut f = Frames::new(record("PC", true, false), Reach::Online);
    f.size = vec2(1280.0, 4000.0);
    f.hosts.list[0].record.settings = Some(serde_json::from_str(r#"{"beam_race":true}"#).unwrap());
    let id = f.hosts.list[0].id;
    f.l.open_host_settings(id, &f.hosts.list[0]);
    f.settle();
    assert!(shows(&f, "1 setting is set here.") && shows(&f, "1 of them has no row on this page; Clear all clears it too."), "{:?}", f.drawn);
    click_text(&mut f, "Clear all for this PC");
    click_text(&mut f, "Clear all");
    assert!(f.l.edit_overrides.is_empty() && shows(&f, "Nothing is set here yet."), "{:?}", f.drawn);
}

/// The settings page in every state worth a look, at 1280x800 (150 %) and 1920x1080:
/// `NIGHTJAR_SHOTS=<dir> cargo test -p nightjar-desktop settings_screenshots -- --ignored`.
#[test]
#[ignore = "writes PNGs; set NIGHTJAR_SHOTS"]
fn settings_screenshots() {
    let Some(dir) = std::env::var_os("NIGHTJAR_SHOTS").map(std::path::PathBuf::from) else { return };
    std::fs::create_dir_all(&dir).expect("the folder for the screenshots");
    for (width, height, ppp) in [(1280.0, 800.0, 1.5), (1920.0, 1080.0, 1.0)] {
        for high in [false, true] {
            let size = format!("{}x{}{}", width as u32, height as u32, if high { "-high-contrast" } else { "" });
            let fresh = || {
                let mut f = home_with_games(1);
                f.size = vec2(width, height);
                f.native_ppp = ppp;
                f.shots = true;
                // The logo went up in frames before shots were kept; load it again so they see it.
                f.l.logo = None;
                f.l.settings.high_contrast = high;
                f.l.settings.fps = 60;
                f.l.settings.bitrate_kbps = 40_000;
                f
            };
            let shoot = |f: &mut Frames, page: &str| {
                f.settle();
                f.settle();
                f.shoot(&dir, &format!("{page}-{size}"));
            };
            let mut f = fresh();
            f.l.go(Screen::Settings);
            shoot(&mut f, "settings");
            f.scroll_to_end();
            shoot(&mut f, "settings-end");
            f.l.settings_view.section = "Controls";
            f.l.settings_view.to_top = true;
            shoot(&mut f, "settings-controls");
            f.scroll_to_end();
            shoot(&mut f, "settings-controls-end");
            if high {
                continue;
            }
            // Every section, top and end.
            for (at, section) in SETTINGS_SECTIONS.iter().enumerate() {
                f.l.settings_view.section = section;
                f.l.settings_view.to_top = true;
                shoot(&mut f, &format!("settings-section-{at}"));
                f.scroll_to_end();
                shoot(&mut f, &format!("settings-section-{at}-end"));
            }
            // Dual screen mode on: Second screen resolution can be changed.
            f.l.settings.dual_screen = true;
            f.l.settings_view.section = "Streaming";
            f.l.settings_view.to_top = true;
            f.settle();
            f.scroll_to_end();
            shoot(&mut f, "settings-dual-screen");
            f.l.settings.dual_screen = false;
            f.l.settings_view.query = "bitrate".into();
            shoot(&mut f, "settings-search");
            f.l.settings_view.query = "teleport".into();
            shoot(&mut f, "settings-search-nothing");
            f.l.settings_view.query.clear();
            f.l.settings_view.section = "App";
            f.l.settings_view.confirm_reset = true;
            f.l.settings_view.to_top = true;
            f.settle();
            f.scroll_to_end();
            shoot(&mut f, "settings-reset-all");
            f.l.settings_view.confirm_reset = false;
            f.l.settings_view.section = "Video";
            // Network tests in the main settings: no paired PC, one, and two with the row that chooses.
            for count in 0..3 {
                let mut f = fresh();
                f.hosts = network_test_pcs(&[("Desk", true, Reach::Online), ("Den", true, Reach::Offline)][..count]);
                if count == 2 {
                    f.hosts.list[0].record.pace = Some(crate::config::PaceCalibration { pace_mbps: 180, client_link_mbps: 1000, host_link_mbps: 2500, wifi: false, ssid: None, qualified: true });
                }
                f.l.go(Screen::Settings);
                f.l.settings_view.section = "Streaming";
                f.l.settings_view.to_top = true;
                shoot(&mut f, &format!("settings-network-tests-{count}"));
                f.scroll_to_end();
                shoot(&mut f, &format!("settings-network-tests-{count}-end"));
            }
            // A PC's settings with two of its own values, over a profile.
            let mut f = fresh();
            let pc = f.hosts.list[0].id;
            let travel = f.l.profiles.create("Travel", serde_json::from_str(r#"{"bitrate_kbps":20000}"#).unwrap()).unwrap();
            f.l.profiles.active = Some(travel);
            f.hosts.list[0].record.settings = Some(serde_json::from_str(r#"{"fps":120,"hdr":true}"#).unwrap());
            f.l.open_host_settings(pc, &f.hosts.list[0]);
            shoot(&mut f, "pc-settings");
            f.l.settings_view.section = "Debug";
            f.l.settings_view.to_top = true;
            shoot(&mut f, "pc-settings-debug");
            f.l.settings_view.section = "Video";
            // A game's settings: one of its own, the rest from the PC, the profile and the main settings.
            let app = nightjar_core::http::App { name: "Racing Game".into(), uuid: "install-0".into(), ..game(1000) };
            f.l.open_app_settings(pc, &f.hosts.list[0], app);
            f.l.edit_overrides.capture(&Settings { codec: CodecPreference::Hevc, ..Settings::default() }, &["codec"]);
            f.l.edit_settings = f.l.edit_overrides.apply(&f.l.inherited_settings);
            shoot(&mut f, "game-settings");
            // The main settings while a profile is in use.
            f.l.go(Screen::Settings);
            shoot(&mut f, "settings-profile-in-use");
        }
    }
}

/// These PCs, each paired or not and answering or not, every one with both network tests.
fn network_test_pcs(pcs: &[(&str, bool, Reach)]) -> Hosts {
    let answer = nightjar_core::http::ServerInfo::parse("<root status_code=\"200\"><uniqueid>uid</uniqueid><state>SUNSHINE_SERVER_FREE</state>\
        <PairStatus>1</PairStatus><RustHostVersion>3.0.0</RustHostVersion><PyroWaveBandwidthProbeBytes>33554432</PyroWaveBandwidthProbeBytes>\
        <PyroWaveUdpProbeVersion>1</PyroWaveUdpProbeVersion><PyroWaveUdpHandshakeVersion>1</PyroWaveUdpHandshakeVersion>\
        <PyroWaveUdpProbeBurstVersion>1</PyroWaveUdpProbeBurstVersion><PyroWaveWireBudgetVersion>1</PyroWaveWireBudgetVersion></root>", 47989).expect("the answer parses");
    let mut hosts = Hosts::without_discovery(pcs.iter().map(|(name, paired, _)| record(name, *paired, false)).collect(), Arc::new(|| {}));
    for (host, (.., reach)) in hosts.list.iter_mut().zip(pcs) {
        (host.reach, host.info) = (*reach, Some(answer.clone()));
    }
    hosts
}

/// The main settings' Streaming section with `pcs` (see `network_test_pcs`), on a tall window.
fn streaming_with_pcs(pcs: &[(&str, bool, Reach)]) -> Frames {
    let mut f = settings_read_with(Settings::default());
    f.hosts = network_test_pcs(pcs);
    f.l.settings_view.section = "Streaming";
    f.settle();
    f
}

/// What the PC row of the main settings' Network tests shows as chosen: the text beside it.
/// The window's top bar names a PC too.
fn chosen_pc(f: &Frames) -> Vec<&str> {
    let row = f.placed.iter().find(|(t, ..)| t == "The paired PC these tests use.").expect("the PC row").1;
    let label = f.placed.iter().find(|(t, r, _)| t == "PC" && r.bottom() <= row.top() + 1.0 && (r.left() - row.left()).abs() < 1.0).expect("its label").1;
    f.placed.iter().filter(|(_, r, _)| r.left() > row.right() && (label.top()..=row.bottom()).contains(&r.center().y)).map(|(t, ..)| t.as_str()).collect()
}

const SPEED_TEST: &str = "Downloads 32 MiB from this PC";
const FRAME_PACE: &str = "Finds a frame pace for PyroWave";

/// A customer looked for the PyroWave calibration in Settings: Network tests is there as on a
/// PC's own settings, the same rows, and the search finds them.
#[test]
fn the_network_tests_are_in_the_main_settings_and_a_pcs_own() {
    let mut f = streaming_with_pcs(&[("Desk", true, Reach::Online)]);
    let id = f.hosts.list[0].id;
    let rows = ["Network tests", "Network speed test", "PyroWave frame pace", "Test speed", "Calibrate"];
    for page in [Screen::Settings, Screen::HostSettings(id)] {
        if page != Screen::Settings {
            f.l.open_host_settings(id, &f.hosts.list[0]);
            f.l.settings_view.section = "Streaming";
            f.settle();
        }
        assert!(rows.iter().all(|row| shows(&f, row)), "{page:?}: {:?}", f.drawn);
        // One PC is the PC: nothing to choose, and nothing to clear before a calibration.
        assert!(!shows(&f, "PC") && !shows(&f, "Clear"), "{page:?}: {:?}", f.drawn);
        assert_eq!((control_disabled(&f, SPEED_TEST), control_disabled(&f, FRAME_PACE)), (Some(false), Some(false)), "{page:?}");
        // They are actions, not settings: no box of a PC's own and no Reset. Every "For this PC"
        // belongs to a setting row.
        let settings = f.l.settings_view.drawn.len();
        let boxes = f.drawn.iter().filter(|t| *t == "For this PC").count();
        assert_eq!(boxes, if page == Screen::Settings { 0 } else { settings }, "{page:?}: {:?}", f.drawn);
        assert!(!shows(&f, "Reset"), "{page:?}: {:?}", f.drawn);
        let commands = click_text(&mut f, "Test speed");
        assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: false }] if host == id), "{page:?}: {} commands", commands.len());
        let commands = click_text(&mut f, "Calibrate");
        assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: true }] if host == id), "{page:?}: {} commands", commands.len());
    }
    // A saved pace says so and can be cleared, from the main settings too.
    f.hosts.list[0].record.pace = Some(crate::config::PaceCalibration { pace_mbps: 180, client_link_mbps: 1000, host_link_mbps: 2500, wifi: false, ssid: None, qualified: true });
    f.l.go(Screen::Settings);
    f.l.settings_view.section = "Streaming";
    f.settle();
    assert!(f.drawn.iter().any(|t| t.starts_with("Saved 180 Mbps · confirmed.")), "{:?}", f.drawn);
    let commands = click_text(&mut f, "Clear");
    assert!(matches!(commands[..], [Command::ClearPace(host)] if host == id), "{} commands", commands.len());

    let streaming = SETTINGS_SECTIONS.iter().position(|s| *s == "Streaming").unwrap();
    for query in ["pyrowave frame pace", "calibrate again", "network tests", "speed test"] {
        f.l.settings_view.query = query.into();
        f.settle();
        assert!(shows(&f, "Network tests") && f.l.settings_view.found[streaming] > 0, "{query}: {:?} {:?}", f.l.settings_view.found, f.drawn);
    }
    f.l.settings_view.query = "pyrowave frame pace".into();
    f.settle();
    assert!(shows(&f, "PyroWave frame pace") && !shows(&f, "Network speed test"), "{:?}", f.drawn);
}

/// With several paired PCs a row chooses the one tested: the first that is online until the
/// user picks another, never one that is not paired.
#[test]
fn the_main_settings_network_tests_choose_among_several_paired_pcs() {
    let mut f = streaming_with_pcs(&[("Loft", false, Reach::Online), ("Desk", true, Reach::Offline), ("Den", true, Reach::Online)]);
    let (loft, desk, den) = (f.hosts.list[0].id, f.hosts.list[1].id, f.hosts.list[2].id);
    assert!(shows(&f, "PC") && shows(&f, "The paired PC these tests use."), "{:?}", f.drawn);
    assert_eq!(chosen_pc(&f), ["Den"], "the paired PC that is online");
    assert_eq!(f.l.settings_view.test_pc, None, "nothing is chosen until the user does");
    let commands = click_text(&mut f, "Test speed");
    assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: false }] if host == den), "{} commands", commands.len());

    // The list has the paired PCs only; choosing one moves both rows to it.
    click_text(&mut f, "Den");
    assert!(shows(&f, "Desk") && !shows(&f, "Loft"), "{:?}", f.drawn);
    click_text(&mut f, "Desk");
    assert_eq!(f.l.settings_view.test_pc, Some(desk));
    assert_eq!(chosen_pc(&f), ["Desk"]);
    // It is off the network: both tests wait for it.
    assert_eq!((control_disabled(&f, SPEED_TEST), control_disabled(&f, FRAME_PACE)), (Some(true), Some(true)));
    assert!(click_text(&mut f, "Test speed").is_empty() && click_text(&mut f, "Calibrate").is_empty());
    f.hosts.list[1].reach = Reach::Online;
    f.settle();
    let commands = click_text(&mut f, "Calibrate");
    assert!(matches!(commands[..], [Command::NetworkTest { host, calibrate: true }] if host == desk), "{} commands", commands.len());

    // The choice is the launcher's, not a setting, and holds only while that PC is paired.
    assert_eq!(f.l.settings, Settings::default());
    let pc = |hosts: &Hosts, chosen| network_test_pc(hosts, chosen).map(|h| h.id);
    assert_eq!(pc(&f.hosts, Some(den)), Some(den));
    assert_eq!(pc(&f.hosts, Some(loft)), Some(desk), "not paired: the first paired PC that is online");
    assert_eq!(pc(&f.hosts, Some(u64::MAX)), Some(desk), "forgotten");
    f.hosts.list[1].reach = Reach::Offline;
    f.hosts.list[2].reach = Reach::Unknown;
    assert_eq!(pc(&f.hosts, None), Some(desk), "none online: the first paired one");

    // One paired PC beside one that is not: nothing to choose.
    let f = streaming_with_pcs(&[("Loft", false, Reach::Online), ("Desk", true, Reach::Online)]);
    assert!(!shows(&f, "PC") && shows(&f, "Network speed test"), "{:?}", f.drawn);
    assert_eq!(network_test_pc(&f.hosts, None).map(|h| h.id), Some(f.hosts.list[1].id));
}

/// Without a paired PC the rows stay where people look for them, turned off.
#[test]
fn the_main_settings_network_tests_are_off_without_a_paired_pc() {
    for pcs in [&[][..], &[("Loft", false, Reach::Online)][..]] {
        let mut f = streaming_with_pcs(pcs);
        assert!(shows(&f, "Network tests") && shows(&f, "Network speed test") && shows(&f, "PyroWave frame pace") && !shows(&f, "PC"), "{:?}", f.drawn);
        assert_eq!((control_disabled(&f, SPEED_TEST), control_disabled(&f, FRAME_PACE)), (Some(true), Some(true)), "{} PCs", pcs.len());
        assert!(click_text(&mut f, "Test speed").is_empty() && click_text(&mut f, "Calibrate").is_empty());
        assert!(network_test_pc(&f.hosts, None).is_none());
        // The rows after them are not turned off with them.
        assert_eq!(control_disabled(&f, "With a second monitor"), Some(false));
    }
}

/// Nightjar #87: system keys can go to the PC only while the stream is full screen. Off as
/// before, a setting of its own right after Send system keys, and nothing to set with that off.
#[test]
fn system_keys_only_in_full_screen_sits_with_send_system_keys() {
    let defaults = Settings::default();
    assert!(defaults.capture_system_keys && !defaults.system_keys_fullscreen_only, "as before: in a window too");
    let old: Settings = serde_json::from_str(r#"{"capture_system_keys":false}"#).unwrap();
    assert!(!old.capture_system_keys && !old.system_keys_fullscreen_only, "a settings file from before");
    let saved = serde_json::to_value(Settings { system_keys_fullscreen_only: true, ..Settings::default() }).unwrap();
    assert_eq!((saved["capture_system_keys"].as_bool(), saved["system_keys_fullscreen_only"].as_bool()), (Some(true), Some(true)));
    for send in [true, false] {
        let mut f = settings_read_with(Settings { capture_system_keys: send, ..Settings::default() });
        f.l.settings_view.section = "Controls";
        f.settle();
        assert!(shows(&f, "Send system keys") && shows(&f, "System keys only in full screen"), "{:?}", f.drawn);
        assert_eq!(control_disabled(&f, "In a window, Alt+Tab"), Some(!send), "Send system keys {send}");
        assert_eq!(control_disabled(&f, "How fast the captured pointer moves."), Some(false), "the next row is its own");
        let rows: Vec<&str> = f.l.settings_view.drawn.iter().map(|(key, _)| *key).collect();
        let at = rows.iter().position(|key| *key == "capture_system_keys").unwrap();
        assert_eq!(rows[at + 1], "system_keys_fullscreen_only", "right after Send system keys");
    }
}

/// Runs `draw` in a bare frame `width` wide.
fn bare_frame<R>(width: f32, draw: impl FnOnce(&mut Ui) -> R) -> R {
    let ctx = egui::Context::default();
    let input = egui::RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(width, 800.0))), ..Default::default() };
    let (mut draw, mut out) = (Some(draw), None);
    let mut output = ctx.run_ui(input, |ui| {
        if let Some(draw) = draw.take() {
            out = Some(draw(ui));
        }
    });
    output.textures_delta.clear();
    out.expect("the frame ran")
}

/// Rows placed one after another are one group across the whole column; a gap starts another.
#[test]
fn rows_that_follow_each_other_are_one_group_across_the_column() {
    let (a, b, c, column_width) = bare_frame(1000.0, |ui| {
        let column_width = ui.available_width();
        let a = group_row(ui, |ui| { ui.label("a"); });
        let b = group_row(ui, |ui| { ui.label("b"); });
        ui.add_space(30.0);
        let c = group_row(ui, |ui| { ui.label("c"); });
        (a, b, c, column_width)
    });
    assert!((a.bottom() - b.top()).abs() < 0.5, "{a:?} then {b:?}");
    assert!((a.width() - column_width).abs() < 0.5, "{} of {column_width}", a.width());
    assert!(c.top() - b.bottom() >= 30.0, "{b:?} then {c:?}");
}

/// A column as wide as a page may ever want still ends with the page: its controls stay on screen.
#[test]
fn a_column_ends_with_the_page_and_stops_at_its_widest_on_a_wide_window() {
    for (window, most) in [(800.0, 800.0), (3000.0, SETTINGS_W)] {
        let right = bare_frame(window, |ui| {
            let mut right = 0.0;
            column(ui, SETTINGS_W, |ui| right = group_row(ui, |ui| { ui.label("a"); }).right());
            right
        });
        assert!(right <= most + 8.0 && right > most - 60.0, "at {window}: {right}");
    }
}

/// A group's heading is the section type as written, not a shouted label.
#[test]
fn a_group_heading_is_written_as_it_reads() {
    let f = settings_with(Settings::default());
    assert!(shows(&f, "Fine-tuning"), "{:?}", f.drawn);
    assert!(!shows(&f, "FINE-TUNING"), "{:?}", f.drawn);
}

/// The switch and the list choice take the sizes the design gives every control.
#[test]
fn a_switch_and_a_list_choice_have_the_designed_size() {
    let (switch, choice) = bare_frame(1000.0, |ui| {
        let switch = toggle(ui, &mut true).rect;
        let choice = Combo::from_id_salt("choice").selected_text("Automatic").show_ui(ui, |_| {}).response.rect;
        (switch, choice)
    });
    assert_eq!(switch.size(), vec2(design_tokens::SIZE_SWITCH_W, design_tokens::SIZE_SWITCH_H));
    assert_eq!(choice.height(), 40.0);
}
